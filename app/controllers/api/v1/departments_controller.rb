module Api
  module V1
    class DepartmentsController < BaseController
      def index
        departments = Department.where(id: current_app_client.accessible_department_ids)
                                .order(:name)
        render json: departments.map { |d| department_json(d) }
      end

      def create
        parent = Department.find(params.require(:parent_id))
        authorize_department!(parent)
        department = Department.create!(name: params.require(:name), parent: parent)
        render json: department_json(department), status: :created
      end

      private

      def department_json(department)
        { id: department.id, name: department.name, parent_id: department.parent_id }
      end
    end
  end
end
