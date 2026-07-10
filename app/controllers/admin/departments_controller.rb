class Admin::DepartmentsController < Admin::BaseController
  def index
    @departments = Department.includes(:parent).order(:name)
  end

  def new
    @department = Department.new
  end

  def create
    @department = Department.new(params.require(:department).permit(:name, :parent_id))
    if @department.save
      redirect_to admin_departments_path, notice: "Department created."
    else
      flash.now[:alert] = @department.errors.full_messages.join(", ")
      render :new, status: :unprocessable_entity
    end
  end

  def destroy
    department = Department.find(params[:id])
    if department.destroy
      redirect_to admin_departments_path, notice: "Department deleted."
    else
      redirect_to admin_departments_path,
                  alert: department.errors.full_messages.join(", ")
    end
  rescue ActiveRecord::InvalidForeignKey
    redirect_to admin_departments_path,
                alert: "Department is still referenced by other records and cannot be deleted."
  end
end
