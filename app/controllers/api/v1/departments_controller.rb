module Api
  module V1
    class DepartmentsController < BaseController
      def index
        departments = Department.where(id: current_app_client.accessible_department_ids)
                                .order(:name)
        render json: paginate(departments).map { |d| department_json(d) }
      end

      def create
        parent = Department.find(require_scalar_param(:parent_id))
        authorize_department!(parent)
        department = Department.create!(name: require_scalar_param(:name), parent: parent)
        render json: department_json(department), status: :created
      rescue ActiveRecord::RecordNotUnique
        # unique-index race: a concurrent create slipped past the validation
        render json: { error: "Name has already been taken" },
               status: :unprocessable_entity
      end

      private

      def department_json(department)
        { id: department.id, name: department.name, parent_id: department.parent_id }
      end
    end
  end
end
